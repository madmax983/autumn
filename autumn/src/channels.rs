//! Named broadcast channel registry for real-time messaging.
//!
//! [`Channels`] provides a lightweight pub-sub primitive backed by
//! [`tokio::sync::broadcast`]. Channels are created lazily on first
//! use and identified by string names.
//!
//! This is the foundation for WebSocket fan-out, SSE event streams,
//! and any pattern where multiple consumers need the same messages.
//!
//! # Examples
//!
//! ```rust
//! use autumn_web::channels::Channels;
//!
//! let channels = Channels::new(32);
//!
//! // Sender and subscriber for the same channel
//! let tx = channels.sender("lobby");
//! let mut rx = channels.subscribe("lobby");
//!
//! tx.send("hello").ok();
//! # // In async context: let msg = rx.recv().await.expect("should receive");
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

/// A registry of named broadcast channels.
///
/// Channels are created lazily when first accessed via [`sender()`](Self::sender)
/// or [`subscribe()`](Self::subscribe). Each channel is a
/// [`tokio::sync::broadcast`] channel with the configured buffer capacity.
///
/// `Channels` is cheaply cloneable (internally `Arc`-wrapped) and is
/// available as a field on [`AppState`](crate::AppState) when the `ws`
/// feature is enabled.
///
/// # Buffer capacity
///
/// The `capacity` sets the number of messages each channel retains for
/// slow receivers. When a receiver falls behind by more than `capacity`
/// messages, it receives a [`RecvError::Lagged`](broadcast::error::RecvError::Lagged)
/// on the next recv, skipping missed messages.
///
/// Choose a capacity that balances memory usage against tolerance for
/// slow consumers. 32–256 is typical for most real-time applications.
#[derive(Clone)]
pub struct Channels {
    inner: Arc<ChannelsInner>,
}

struct ChannelsInner {
    capacity: usize,
    registry: Mutex<HashMap<String, Arc<broadcast::Sender<ChannelMessage>>>>,
}

/// A message sent through a broadcast channel.
///
/// Currently wraps a `String`. Future versions may support typed
/// or binary messages via a `MessageChannel<T>` layer.
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

/// A sender handle for a broadcast channel.
///
/// Obtained from [`Channels::sender()`]. Cheaply cloneable.
/// Sending to a channel with no active subscribers silently succeeds.
#[derive(Clone)]
pub struct Sender {
    inner: Arc<broadcast::Sender<ChannelMessage>>,
}

impl Sender {
    /// Broadcast a message to all current subscribers of this channel.
    ///
    /// Returns `Ok(receiver_count)` on success, or `Err` if there are
    /// no active subscribers. The error is typically non-fatal — it just
    /// means no one is listening.
    ///
    /// # Errors
    ///
    /// Returns the unsent message if there are no active receivers.
    pub fn send(
        &self,
        msg: impl Into<ChannelMessage>,
    ) -> Result<usize, broadcast::error::SendError<ChannelMessage>> {
        self.inner.send(msg.into())
    }

    /// Returns the current number of active subscribers.
    #[must_use]
    pub fn receiver_count(&self) -> usize {
        self.inner.receiver_count()
    }
}

/// A subscriber handle for a broadcast channel.
///
/// Obtained from [`Channels::subscribe()`]. Each subscriber receives
/// its own copy of every message sent after it subscribed.
pub struct Subscriber {
    inner: broadcast::Receiver<ChannelMessage>,
}

impl Subscriber {
    /// Receive the next message from the channel.
    ///
    /// Waits until a message is available. Returns
    /// [`RecvError::Lagged(n)`](broadcast::error::RecvError::Lagged)
    /// if this subscriber fell behind by `n` messages.
    ///
    /// # Errors
    ///
    /// Returns `RecvError::Closed` if all senders have been dropped,
    /// or `RecvError::Lagged(n)` if messages were skipped.
    pub async fn recv(&mut self) -> Result<ChannelMessage, broadcast::error::RecvError> {
        self.inner.recv().await
    }
}

impl Channels {
    /// Create a new channel registry with the given per-channel buffer capacity.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::channels::Channels;
    ///
    /// let channels = Channels::new(64); // 64-message buffer per channel
    /// ```
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        // tokio::sync::broadcast channel capacity must be > 0 and <= usize::MAX / 2
        // Furthermore, allocating huge capacities will OOM the process.
        // Cap it at a reasonable maximum for an application, like 16384, and min 1.
        Self {
            inner: Arc::new(ChannelsInner {
                capacity: capacity.clamp(1, 16384),
                registry: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Get or create a sender for the named channel.
    ///
    /// If the channel doesn't exist yet, it's created with the registry's
    /// default buffer capacity.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (indicates a prior panic
    /// while holding the lock).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::channels::Channels;
    ///
    /// let channels = Channels::new(32);
    /// let tx = channels.sender("notifications");
    /// tx.send("new message").ok();
    /// ```
    #[must_use]
    pub fn sender(&self, name: &str) -> Sender {
        let mut registry = self.inner.registry.lock().expect("channels lock poisoned");

        // ⚡ Bolt Optimization: Use get() first to avoid allocating a String key
        // on every lookup for channels that already exist.
        #[allow(clippy::option_if_let_else)]
        let tx = if let Some(tx) = registry.get(name) {
            Arc::clone(tx)
        } else {
            let capacity = std::cmp::max(1, self.inner.capacity);
            let tx = Arc::new(broadcast::channel(capacity).0);
            registry.insert(name.to_owned(), Arc::clone(&tx));
            tx
        };

        let sender = Sender { inner: tx };
        drop(registry);
        sender
    }

    /// Subscribe to the named channel.
    ///
    /// If the channel doesn't exist yet, it's created with the registry's
    /// default buffer capacity. The subscriber receives all messages sent
    /// **after** this call.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn_web::channels::Channels;
    ///
    /// let channels = Channels::new(32);
    /// let mut rx = channels.subscribe("notifications");
    /// // In async context: let msg = rx.recv().await?;
    /// ```
    #[must_use]
    pub fn subscribe(&self, name: &str) -> Subscriber {
        let mut registry = self.inner.registry.lock().expect("channels lock poisoned");

        // ⚡ Bolt Optimization: Use get() first to avoid allocating a String key
        // on every lookup for channels that already exist.
        #[allow(clippy::option_if_let_else)]
        let tx = if let Some(tx) = registry.get(name) {
            Arc::clone(tx)
        } else {
            let capacity = std::cmp::max(1, self.inner.capacity);
            let tx = Arc::new(broadcast::channel(capacity).0);
            registry.insert(name.to_owned(), Arc::clone(&tx));
            tx
        };

        let subscriber = Subscriber {
            inner: tx.subscribe(),
        };
        drop(registry);
        subscriber
    }

    /// Returns the number of active channels in the registry.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        let registry = self.inner.registry.lock().expect("channels lock poisoned");
        registry.len()
    }

    /// Remove channels with no active senders or receivers.
    ///
    /// Call this periodically if your application creates many
    /// short-lived channels (e.g., per-room channels in a chat app).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn gc(&self) {
        let mut registry = self.inner.registry.lock().expect("channels lock poisoned");
        registry.retain(|_, tx| tx.receiver_count() > 0 || Arc::strong_count(tx) > 1);
    }

    /// Get a snapshot of all active channels and their subscriber counts.
    ///
    /// Returns a `HashMap` mapping channel names to their current active receiver count.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, usize> {
        let registry = self.inner.registry.lock().expect("channels lock poisoned");
        registry
            .iter()
            .map(|(name, tx)| (name.clone(), tx.receiver_count()))
            .collect()
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
        assert_eq!(snap.get("empty"), Some(&0));
        assert_eq!(snap.get("one"), Some(&1));
        assert_eq!(snap.get("two"), Some(&2));
        assert_eq!(snap.len(), 3);
    }

    #[test]
    fn gc_removes_dead_channels() {
        let channels = Channels::new(16);
        let _tx = channels.sender("alive");
        // Create and immediately drop subscriber for "dead" channel
        {
            let _tx = channels.sender("dead");
        }
        assert_eq!(channels.channel_count(), 2);
        channels.gc();
        // "alive" has an active sender (_tx), so it is kept (count = 1).
        // "dead" has 0 receivers and 0 active senders (dropped), so it gets cleaned.
        assert_eq!(channels.channel_count(), 1);
    }
}
