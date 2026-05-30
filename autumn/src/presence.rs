//! Distributed presence tracking layered on top of [`Channels`].
//!
//! [`Presence`] provides per-topic membership tracking with automatic join/leave
//! event broadcasting, connection-scoped leases, and configurable TTL-based
//! eviction for stale entries. It is the Autumn equivalent of Phoenix Presence.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//! use serde_json::json;
//!
//! #[get("/post/{id}/viewers")]
//! async fn viewers(presence: Presence, path: Path<i64>) -> impl IntoResponse {
//!     let topic = format!("post:{}", *path);
//!     let entries = presence.list(&topic);
//!     Json(entries)
//! }
//!
//! #[get("/post/{id}/view")]
//! async fn track_view(presence: Presence, path: Path<i64>) -> impl IntoResponse {
//!     let topic = format!("post:{}", *path);
//!     // In a real app, use the authenticated user's ID as the key
//!     let _handle = presence.track(topic, "user_123", json!({"role": "viewer"}));
//!     // _handle kept alive; on drop, removes presence and broadcasts leave
//!     StatusCode::OK
//! }
//! ```
//!
//! [`Channels`]: crate::channels::Channels

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::channels::Channels;

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

fn next_connection_id() -> u64 {
    NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

/// Default TTL for presence entries, in seconds.
const DEFAULT_TTL_SECS: u64 = 30;

#[derive(Clone)]
struct ConnectionPresence {
    connection_id: u64,
    meta: JsonValue,
    last_heartbeat: Instant,
}

struct PresenceInner {
    // BTreeMap for the per-key layer so list() returns entries in stable key
    // order — deterministic ordering prevents flaky tests and predictable UI.
    entries: HashMap<String, std::collections::BTreeMap<String, Vec<ConnectionPresence>>>,
    ttl: Duration,
}

impl PresenceInner {
    fn new(ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            ttl,
        }
    }

    fn add(&mut self, topic: &str, key: &str, connection_id: u64, meta: JsonValue) {
        self.entries
            .entry(topic.to_owned())
            .or_default()
            .entry(key.to_owned())
            .or_default()
            .push(ConnectionPresence {
                connection_id,
                meta,
                last_heartbeat: Instant::now(),
            });
    }

    /// Remove the given connection.  Returns `true` if the key is now completely
    /// absent from the topic (i.e. this was its last active connection), which
    /// is the condition under which a `Leave` event should be broadcast.
    fn remove(&mut self, topic: &str, key: &str, connection_id: u64) -> bool {
        let mut key_fully_removed = false;
        if let Some(by_key) = self.entries.get_mut(topic) {
            if let Some(conns) = by_key.get_mut(key) {
                conns.retain(|c| c.connection_id != connection_id);
                if conns.is_empty() {
                    by_key.remove(key);
                    key_fully_removed = true;
                }
            }
            if by_key.is_empty() {
                self.entries.remove(topic);
            }
        }
        key_fully_removed
    }

    fn list(&self, topic: &str) -> Vec<PresenceEntry> {
        let Some(by_key) = self.entries.get(topic) else {
            return Vec::new();
        };
        by_key
            .iter()
            .map(|(key, conns)| PresenceEntry {
                key: key.clone(),
                metas: conns.iter().map(|c| c.meta.clone()).collect(),
            })
            .collect()
    }

    fn refresh(&mut self, topic: &str, key: &str, connection_id: u64) {
        if let Some(by_key) = self.entries.get_mut(topic) {
            if let Some(conns) = by_key.get_mut(key) {
                for c in conns.iter_mut() {
                    if c.connection_id == connection_id {
                        c.last_heartbeat = Instant::now();
                    }
                }
            }
        }
    }

    fn sweep_expired(&mut self) -> Vec<(String, String)> {
        let ttl = self.ttl;
        let now = Instant::now();
        let mut removed = Vec::new();

        self.entries.retain(|topic, by_key| {
            by_key.retain(|key, conns| {
                conns.retain(|c| now.duration_since(c.last_heartbeat) < ttl);
                if conns.is_empty() {
                    removed.push((topic.clone(), key.clone()));
                    false
                } else {
                    true
                }
            });
            !by_key.is_empty()
        });

        removed
    }
}

/// A merged presence entry for one key on a topic (Phoenix Presence.list/1 semantics).
///
/// When the same `key` is tracked from multiple connections, those connections
/// are collapsed into one `PresenceEntry` with one `meta` value per connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceEntry {
    /// The unique identifier for this presence slot (e.g., user ID or session ID).
    pub key: String,
    /// One metadata payload per active connection for this key.
    pub metas: Vec<JsonValue>,
}

/// Event emitted on `presence:{topic}` when a member joins or leaves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PresenceEvent {
    /// A new connection was registered for `key`.
    Join {
        /// The key that joined.
        key: String,
        /// Metadata for this connection.
        meta: JsonValue,
    },
    /// A connection was removed for `key`.
    Leave {
        /// The key that left.
        key: String,
    },
}

/// Distributed presence tracker layered on top of [`Channels`].
///
/// `Presence` is a request extractor — declare it as a handler argument and
/// the framework injects a clone backed by the shared process-level store.
///
/// # Backends
///
/// - **In-process** (default): all state lives in `Arc<Mutex<…>>` shared within
///   the process. Perfect for development and single-replica production.
/// - **Redis**: join/leave events are broadcast through the Redis pub/sub backend,
///   so every replica sees them. Each replica maintains its own local membership
///   view; replicas converge as heartbeats and events propagate.
///
/// # Stale entry eviction
///
/// Entries are evicted by a periodic background sweep (default 30 s) if the
/// heartbeat is not refreshed. Call [`PresenceHandle::refresh`] from your
/// heartbeat loop to extend the lease.
#[derive(Clone)]
pub struct Presence {
    inner: Arc<Mutex<PresenceInner>>,
    channels: Channels,
}

impl Presence {
    /// Create a presence tracker backed by the given channel registry, with
    /// the default TTL of 30 seconds.
    #[must_use]
    pub fn new(channels: Channels) -> Self {
        Self::with_ttl(channels, Duration::from_secs(DEFAULT_TTL_SECS))
    }

    /// Create a presence tracker with a custom TTL for stale-entry eviction.
    #[must_use]
    pub fn with_ttl(channels: Channels, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PresenceInner::new(ttl))),
            channels,
        }
    }

    /// Register a presence entry for `key` on `topic` with metadata `meta`.
    ///
    /// A join event is immediately broadcast on the derived channel
    /// `presence:{topic}`. The returned [`PresenceHandle`] keeps the entry
    /// alive; when it is dropped a leave event is broadcast and the entry is
    /// removed.
    pub fn track(
        &self,
        topic: impl Into<String>,
        key: impl Into<String>,
        meta: impl Into<JsonValue>,
    ) -> PresenceHandle {
        let topic = topic.into();
        let key = key.into();
        let meta = meta.into();
        let connection_id = next_connection_id();

        {
            let mut inner = self.inner.lock().expect("presence lock poisoned");
            inner.add(&topic, &key, connection_id, meta.clone());
        }

        let event = PresenceEvent::Join {
            key: key.clone(),
            meta,
        };
        self.publish_event(&topic, &event);

        PresenceHandle {
            topic,
            key,
            connection_id,
            inner: Arc::clone(&self.inner),
            channels: self.channels.clone(),
        }
    }

    /// Return the current merged presence list for `topic`.
    ///
    /// Connections with the same `key` are collapsed into one [`PresenceEntry`]
    /// with a list of `metas` — one per active connection (Phoenix
    /// `Presence.list/1` semantics).
    #[must_use]
    pub fn list(&self, topic: &str) -> Vec<PresenceEntry> {
        self.inner
            .lock()
            .expect("presence lock poisoned")
            .list(topic)
    }

    /// Evict presence entries whose heartbeat has not been refreshed within the
    /// configured TTL, and broadcast a leave event for each.
    ///
    /// This is called automatically by the background sweep task started during
    /// `AppBuilder::run`. You only need to call it manually in tests or custom
    /// task runners.
    pub fn sweep_expired(&self) {
        let removed = {
            let mut inner = self.inner.lock().expect("presence lock poisoned");
            inner.sweep_expired()
        };
        for (topic, key) in removed {
            self.publish_event(&topic, &PresenceEvent::Leave { key });
        }
    }

    fn publish_event(&self, topic: &str, event: &PresenceEvent) {
        let json = serde_json::to_string(event).unwrap_or_default();
        let _ = self.channels.publish(&format!("presence:{topic}"), json);
    }
}

/// RAII guard for a tracked presence entry.
///
/// When dropped, the entry is removed from the presence store and a leave event
/// is broadcast on `presence:{topic}`.
pub struct PresenceHandle {
    topic: String,
    key: String,
    connection_id: u64,
    inner: Arc<Mutex<PresenceInner>>,
    channels: Channels,
}

impl PresenceHandle {
    /// Refresh the heartbeat for this entry, extending its lease by the full TTL.
    ///
    /// Call from your WebSocket ping loop or SSE keep-alive to prevent eviction.
    pub fn refresh(&self) {
        let mut inner = self.inner.lock().expect("presence lock poisoned");
        inner.refresh(&self.topic, &self.key, self.connection_id);
    }

    /// Returns the topic this presence is tracked on.
    #[must_use]
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Returns the key for this presence entry.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl Drop for PresenceHandle {
    fn drop(&mut self) {
        let key_fully_removed = {
            let mut inner = self.inner.lock().expect("presence lock poisoned");
            inner.remove(&self.topic, &self.key, self.connection_id)
        };

        // Only broadcast Leave when this was the last connection for the key.
        // If the same user still has other tabs open their key remains present,
        // so emitting a Leave would incorrectly signal they have gone offline.
        if key_fully_removed {
            let event = PresenceEvent::Leave {
                key: self.key.clone(),
            };
            let json = serde_json::to_string(&event).unwrap_or_default();
            let _ = self
                .channels
                .publish(&format!("presence:{}", self.topic), json);
        }
    }
}

impl axum::extract::FromRequestParts<crate::state::AppState> for Presence {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        _parts: &mut http::request::Parts,
        state: &crate::state::AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(state.presence().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_presence() -> Presence {
        let channels = Channels::new(16);
        Presence::new(channels)
    }

    // ── RED PHASE TESTS ──────────────────────────────────────────────────────

    #[test]
    fn track_adds_one_entry() {
        let presence = make_presence();
        let _handle = presence.track("room:1", "alice", serde_json::json!({"color": "blue"}));
        let entries = presence.list("room:1");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "alice");
        assert_eq!(entries[0].metas.len(), 1);
        assert_eq!(entries[0].metas[0]["color"], "blue");
    }

    #[test]
    fn drop_handle_removes_entry() {
        let presence = make_presence();
        {
            let _handle = presence.track("room:1", "alice", serde_json::json!({}));
            assert_eq!(presence.list("room:1").len(), 1);
        }
        assert_eq!(presence.list("room:1").len(), 0);
    }

    #[test]
    fn same_key_multiple_connections_collapsed() {
        let presence = make_presence();
        let _h1 = presence.track("room:1", "alice", serde_json::json!({"tab": 1}));
        let _h2 = presence.track("room:1", "alice", serde_json::json!({"tab": 2}));
        let entries = presence.list("room:1");
        assert_eq!(entries.len(), 1, "same key should collapse into one entry");
        assert_eq!(entries[0].key, "alice");
        assert_eq!(entries[0].metas.len(), 2);
    }

    #[test]
    fn dropping_one_connection_keeps_other() {
        let presence = make_presence();
        let _h1 = presence.track("room:1", "alice", serde_json::json!({"tab": 1}));
        {
            let _h2 = presence.track("room:1", "alice", serde_json::json!({"tab": 2}));
            assert_eq!(presence.list("room:1")[0].metas.len(), 2);
        }
        let entries = presence.list("room:1");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].metas.len(), 1);
    }

    #[test]
    fn different_keys_are_separate_entries() {
        let presence = make_presence();
        let _h1 = presence.track("room:1", "alice", serde_json::json!({}));
        let _h2 = presence.track("room:1", "bob", serde_json::json!({}));
        let mut entries = presence.list("room:1");
        entries.sort_by_key(|e| e.key.clone());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "alice");
        assert_eq!(entries[1].key, "bob");
    }

    #[test]
    fn list_unknown_topic_returns_empty() {
        let presence = make_presence();
        assert!(presence.list("nonexistent").is_empty());
    }

    #[test]
    fn sweep_removes_stale_entries() {
        let channels = Channels::new(16);
        let presence = Presence::with_ttl(channels, Duration::from_nanos(1));

        // track and then wait just past TTL
        let _handle = presence.track("room:1", "alice", serde_json::json!({}));
        std::thread::sleep(Duration::from_millis(1));

        presence.sweep_expired();
        assert!(presence.list("room:1").is_empty());
    }

    #[test]
    fn sweep_respects_refreshed_entries() {
        let channels = Channels::new(16);
        let presence = Presence::with_ttl(channels, Duration::from_millis(50));

        let handle = presence.track("room:1", "alice", serde_json::json!({}));
        std::thread::sleep(Duration::from_millis(30));
        handle.refresh(); // extend lease
        std::thread::sleep(Duration::from_millis(30));

        presence.sweep_expired();
        // total elapsed since last refresh < TTL
        assert_eq!(presence.list("room:1").len(), 1);
    }

    #[test]
    fn presence_handle_exposes_topic_and_key() {
        let presence = make_presence();
        let handle = presence.track("chat:42", "user_7", serde_json::json!({}));
        assert_eq!(handle.topic(), "chat:42");
        assert_eq!(handle.key(), "user_7");
    }

    // ── CHANNEL EVENT TESTS ──────────────────────────────────────────────────

    #[tokio::test]
    async fn no_leave_event_while_other_connections_remain() {
        let channels = Channels::new(16);
        let presence = Presence::new(channels.clone());
        let mut rx = channels.subscribe("presence:room:1");

        let h1 = presence.track("room:1", "alice", serde_json::json!({"tab": 1}));
        let _h2 = presence.track("room:1", "alice", serde_json::json!({"tab": 2}));
        // drain both join events
        let _ = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        let _ = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;

        // Drop h1 — alice still has h2 open, so no Leave should be broadcast.
        drop(h1);
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(
            result.is_err(),
            "no Leave event should be emitted while another connection is open"
        );
        // Alice is still present.
        assert_eq!(presence.list("room:1")[0].metas.len(), 1);
    }

    #[tokio::test]
    async fn join_event_broadcast_on_track() {
        let channels = Channels::new(16);
        let mut rx = channels.subscribe("presence:room:1");
        let presence = Presence::new(channels);

        let _handle = presence.track("room:1", "alice", serde_json::json!({"name": "Alice"}));

        let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("join event timed out")
            .expect("channel closed");

        let event: serde_json::Value = serde_json::from_str(msg.as_str()).unwrap();
        assert_eq!(event["type"], "join");
        assert_eq!(event["key"], "alice");
        assert_eq!(event["meta"]["name"], "Alice");
    }

    #[tokio::test]
    async fn leave_event_broadcast_on_drop() {
        let channels = Channels::new(16);
        let presence = Presence::new(channels.clone());
        let mut rx = channels.subscribe("presence:room:1");

        {
            let _handle = presence.track("room:1", "alice", serde_json::json!({}));
            // drain join event
            let _ = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        }

        let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("leave event timed out")
            .expect("channel closed");

        let event: serde_json::Value = serde_json::from_str(msg.as_str()).unwrap();
        assert_eq!(event["type"], "leave");
        assert_eq!(event["key"], "alice");
    }

    #[tokio::test]
    async fn sweep_broadcasts_leave_events() {
        let channels = Channels::new(16);
        let presence = Presence::with_ttl(channels.clone(), Duration::from_nanos(1));
        let mut rx = channels.subscribe("presence:room:1");

        presence.track("room:1", "alice", serde_json::json!({}));
        // drain join event
        let _ = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;

        std::thread::sleep(Duration::from_millis(1));
        presence.sweep_expired();

        let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("sweep leave event timed out")
            .expect("channel closed");

        let event: serde_json::Value = serde_json::from_str(msg.as_str()).unwrap();
        assert_eq!(event["type"], "leave");
    }

    #[test]
    fn presence_event_join_serializes_correctly() {
        let event = PresenceEvent::Join {
            key: "alice".to_owned(),
            meta: serde_json::json!({"role": "admin"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "join");
        assert_eq!(parsed["key"], "alice");
        assert_eq!(parsed["meta"]["role"], "admin");
    }

    #[test]
    fn presence_event_leave_serializes_correctly() {
        let event = PresenceEvent::Leave {
            key: "alice".to_owned(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "leave");
        assert_eq!(parsed["key"], "alice");
    }

    #[test]
    fn presence_event_round_trips() {
        let events = vec![
            PresenceEvent::Join {
                key: "bob".to_owned(),
                meta: serde_json::json!({"tab": 2}),
            },
            PresenceEvent::Leave {
                key: "bob".to_owned(),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let _parsed: PresenceEvent = serde_json::from_str(&json).unwrap();
        }
    }
}
