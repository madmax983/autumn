//! Horizontal database sharding.
//!
//! Autumn routes sharded data in two steps: a routing key (typically the
//! tenant id) hashes onto a fixed set of **logical slots**
//! (`database.slot_count`, default 64), and each slot maps to one physical
//! shard via the `[[database.shards]]` configuration. The key→slot hash is
//! a permanent contract — it is deterministic across processes, replicas,
//! and Autumn versions — while the slot→shard map is plain configuration.
//! Resharding therefore means moving whole slots between shards and
//! flipping the map, never rehashing keys.
//!
//! Each shard is a full [`DatabaseTopology`] (primary + optional read
//! replica), so the primary/replica story composes with sharding.
//!
//! Framework state (jobs, scheduler locks, sessions, feature flags) is
//! **not** sharded; it lives on the control topology configured by
//! `database.primary_url`/`database.url`.
//!
//! # Example
//!
//! ```toml
//! [database]
//! primary_url = "postgres://db-control/app"
//!
//! [[database.shards]]
//! name = "shard0"
//! primary_url = "postgres://db-shard0/app"
//! slots = ["0-31"]
//!
//! [[database.shards]]
//! name = "shard1"
//! primary_url = "postgres://db-shard1/app"
//! slots = ["32-63"]
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::deadpool::Pool;

use crate::config::{ConfigError, DatabaseConfig, ReplicaFallback};
use crate::db::{DatabaseTopology, PoolError};
use crate::error::AutumnError;

/// Index of a physical shard within the configured shard set.
///
/// Stable only for a given configuration; use [`Shard::name`] for
/// identity that survives configuration edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShardId(pub usize);

/// A logical routing slot in `0..database.slot_count`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotId(pub u16);

/// Borrowed routing key.
///
/// Tenant ids in Autumn are strings, so [`ShardKey::Str`] is the common
/// variant; `Int` and `Bytes` cover numeric primary keys and UUIDs
/// (`ShardKey::from(uuid.as_bytes())`).
#[derive(Debug, Clone, Copy)]
pub enum ShardKey<'a> {
    /// Numeric key (e.g. a `BIGINT` primary key).
    Int(i64),
    /// Textual key (e.g. a tenant id).
    Str(&'a str),
    /// Raw bytes (e.g. a UUID).
    Bytes(&'a [u8]),
}

impl From<i64> for ShardKey<'_> {
    fn from(key: i64) -> Self {
        Self::Int(key)
    }
}

impl From<i32> for ShardKey<'_> {
    fn from(key: i32) -> Self {
        Self::Int(i64::from(key))
    }
}

impl<'a> From<&'a str> for ShardKey<'a> {
    fn from(key: &'a str) -> Self {
        Self::Str(key)
    }
}

impl<'a> From<&'a String> for ShardKey<'a> {
    fn from(key: &'a String) -> Self {
        Self::Str(key)
    }
}

impl<'a> From<&'a [u8]> for ShardKey<'a> {
    fn from(key: &'a [u8]) -> Self {
        Self::Bytes(key)
    }
}

impl<'a> From<&'a [u8; 16]> for ShardKey<'a> {
    fn from(key: &'a [u8; 16]) -> Self {
        Self::Bytes(key)
    }
}

// ── Deterministic key hashing ────────────────────────────────────────────────
//
// The key→slot function is a PERMANENT CONTRACT: every process, replica,
// and future Autumn version must route the same key to the same slot, or
// data written by one replica becomes invisible to another. That rules out
// std's SipHash (randomly keyed per process). FNV-1a and splitmix64 are
// fixed, well-known functions; the golden-vector tests below pin their
// output forever.

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// splitmix64 finalizer — mixes integer keys so that sequential ids
/// spread uniformly across slots.
const fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Deterministic 64-bit hash of a routing key.
#[must_use]
fn key_hash64(key: ShardKey<'_>) -> u64 {
    match key {
        #[allow(clippy::cast_sign_loss)]
        ShardKey::Int(value) => splitmix64(value as u64),
        ShardKey::Str(value) => fnv1a_64(value.as_bytes()),
        ShardKey::Bytes(value) => fnv1a_64(value),
    }
}

/// Map a routing key onto a logical slot in `0..slot_count`.
///
/// This function is deterministic across processes and versions; see the
/// module docs. `slot_count` must be non-zero (validated at config load).
#[must_use]
pub fn slot_for_key(key: ShardKey<'_>, slot_count: u16) -> SlotId {
    debug_assert!(slot_count > 0, "slot_count is validated at config load");
    let hash = key_hash64(key);
    #[allow(clippy::cast_possible_truncation)]
    SlotId((hash % u64::from(slot_count.max(1))) as u16)
}

// ── Router ───────────────────────────────────────────────────────────────────

/// Pluggable shard routing strategy.
///
/// The default [`HashShardRouter`] hashes the key onto a logical slot and
/// resolves the slot's owner from configuration. Implement this trait for
/// directory/lookup routing (e.g. a control-plane table mapping tenants to
/// shards, with hot "whale" tenants pinned to dedicated shards) and
/// install it with
/// [`AppBuilder::with_shard_router`](crate::app::AppBuilder::with_shard_router).
///
/// Routing is async so directory routers can consult a cache or the
/// control database. Custom routers can still compose with the hash via
/// [`ShardSet::slot_for_key`] and [`ShardSet::shard_for_slot`].
pub trait ShardRouter: Send + Sync + 'static {
    /// Resolve the shard that owns `key`.
    fn route<'a>(
        &'a self,
        key: ShardKey<'a>,
        shards: &'a ShardSet,
    ) -> futures::future::BoxFuture<'a, Result<ShardId, AutumnError>>;
}

/// Default router: key → logical slot (deterministic hash) → shard
/// (configured slot map).
#[derive(Debug, Default, Clone, Copy)]
pub struct HashShardRouter;

impl ShardRouter for HashShardRouter {
    fn route<'a>(
        &'a self,
        key: ShardKey<'a>,
        shards: &'a ShardSet,
    ) -> futures::future::BoxFuture<'a, Result<ShardId, AutumnError>> {
        let slot = shards.slot_for_key(key);
        Box::pin(std::future::ready(
            shards
                .inner
                .slot_map
                .get(usize::from(slot.0))
                .map(|&idx| ShardId(idx))
                .ok_or_else(|| {
                    AutumnError::service_unavailable_msg(format!(
                        "slot {} has no shard assigned (slot map inconsistent)",
                        slot.0
                    ))
                }),
        ))
    }
}

// ── Shard runtime state ──────────────────────────────────────────────────────

/// Mutable per-shard replica readiness, updated by the per-shard health
/// indicator on readiness probes (mirrors the control replica's
/// [`ProbeState`](crate::probe::ProbeState) lifecycle).
#[derive(Debug)]
pub(crate) struct ShardRuntime {
    replica_fallback: ReplicaFallback,
    replica_configured: bool,
    connection_ready: AtomicBool,
    migrations_ready: AtomicBool,
    detail: std::sync::RwLock<Option<String>>,
    /// `(primary_url, replica_url)` for re-running the migration parity
    /// check from the per-shard health indicator. `None` when the app
    /// registered no migrations.
    migration_check: std::sync::RwLock<Option<(String, String)>>,
    /// When the parity comparison last ran, for throttling: unlike the
    /// pooled connectivity check, parity opens fresh synchronous
    /// connections to both roles, so it must not run on every probe.
    parity_checked_at: std::sync::Mutex<Option<std::time::Instant>>,
}

/// Minimum interval between migration parity re-checks per shard.
///
/// Readiness probes fire every few seconds per replica; the parity check
/// opens fresh synchronous connections to the shard's primary *and*
/// replica, so running it per probe per shard would exhaust Postgres
/// connection limits as shard counts grow.
const PARITY_RECHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

// Mutators are driven by startup migration parity checks and the
// per-shard health indicators; some are exercised only by tests until
// the health wiring lands.
#[cfg_attr(not(test), allow(dead_code))]
impl ShardRuntime {
    fn new(replica_fallback: ReplicaFallback, replica_configured: bool) -> Self {
        Self {
            replica_fallback,
            replica_configured,
            connection_ready: AtomicBool::new(false),
            migrations_ready: AtomicBool::new(true),
            detail: std::sync::RwLock::new(
                replica_configured.then(|| "replica has not passed a readiness check".to_owned()),
            ),
            migration_check: std::sync::RwLock::new(None),
            parity_checked_at: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn configure_migration_check(&self, primary_url: String, replica_url: String) {
        *self
            .migration_check
            .write()
            .expect("shard runtime lock poisoned") = Some((primary_url, replica_url));
    }

    fn migration_check(&self) -> Option<(String, String)> {
        self.migration_check
            .read()
            .expect("shard runtime lock poisoned")
            .clone()
    }

    /// Whether the throttle window has elapsed; claims the slot when it
    /// has, so concurrent probes run at most one parity check per window.
    pub(crate) fn parity_check_due(&self) -> bool {
        let mut checked_at = self
            .parity_checked_at
            .lock()
            .expect("shard runtime lock poisoned");
        if checked_at.is_none_or(|at| at.elapsed() >= PARITY_RECHECK_INTERVAL) {
            *checked_at = Some(std::time::Instant::now());
            true
        } else {
            false
        }
    }

    fn replica_ready(&self) -> bool {
        self.connection_ready.load(Ordering::Relaxed)
            && self.migrations_ready.load(Ordering::Relaxed)
    }

    fn refresh_detail(&self) {
        if self.replica_ready() {
            *self.detail.write().expect("shard runtime lock poisoned") = None;
        }
    }

    pub(crate) fn mark_replica_connection_ready(&self) {
        self.connection_ready.store(true, Ordering::Relaxed);
        self.refresh_detail();
    }

    pub(crate) fn mark_replica_connection_unready(&self, detail: impl Into<String>) {
        self.connection_ready.store(false, Ordering::Relaxed);
        *self.detail.write().expect("shard runtime lock poisoned") = Some(detail.into());
    }

    pub(crate) fn mark_replica_migrations_ready(&self) {
        self.migrations_ready.store(true, Ordering::Relaxed);
        self.refresh_detail();
    }

    pub(crate) fn mark_replica_migrations_unready(&self, detail: impl Into<String>) {
        self.migrations_ready.store(false, Ordering::Relaxed);
        *self.detail.write().expect("shard runtime lock poisoned") = Some(detail.into());
    }

    pub(crate) fn detail(&self) -> Option<String> {
        self.detail
            .read()
            .expect("shard runtime lock poisoned")
            .clone()
    }
}

// ── Shard / ShardSet ─────────────────────────────────────────────────────────

/// One physical shard: a named [`DatabaseTopology`] plus its slot
/// assignment and runtime replica state.
#[derive(Clone)]
pub struct Shard {
    name: Arc<str>,
    id: ShardId,
    slots: Arc<[u16]>,
    topology: DatabaseTopology,
    runtime: Arc<ShardRuntime>,
}

impl Shard {
    /// Stable shard name from configuration.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Position of this shard in the configured set.
    #[must_use]
    pub const fn id(&self) -> ShardId {
        self.id
    }

    /// Logical slots owned by this shard, in ascending order.
    #[must_use]
    pub fn slots(&self) -> &[u16] {
        &self.slots
    }

    /// This shard's primary/replica pool topology.
    #[must_use]
    pub const fn topology(&self) -> &DatabaseTopology {
        &self.topology
    }

    /// This shard's primary/write pool.
    #[must_use]
    pub const fn primary_pool(&self) -> &Pool<AsyncPgConnection> {
        self.topology.primary()
    }

    /// This shard's replica pool, when configured.
    #[must_use]
    pub const fn replica_pool(&self) -> Option<&Pool<AsyncPgConnection>> {
        self.topology.replica()
    }

    /// Pool for read-only work, honoring this shard's `replica_fallback`
    /// and runtime replica readiness (mirrors
    /// [`AppState::read_pool`](crate::AppState::read_pool)):
    ///
    /// - no replica configured → the primary pool;
    /// - replica configured and ready → the replica pool;
    /// - replica unready, fallback `primary` → the primary pool;
    /// - replica unready, fallback `fail_readiness` → `None`.
    #[must_use]
    pub fn read_pool(&self) -> Option<&Pool<AsyncPgConnection>> {
        self.read_pool_with_role().map(|(pool, _)| pool)
    }

    /// [`read_pool`](Self::read_pool) plus the role label of the returned
    /// pool, for interceptor/metric naming.
    pub(crate) fn read_pool_with_role(&self) -> Option<(&Pool<AsyncPgConnection>, &'static str)> {
        if !self.runtime.replica_configured {
            return Some((self.topology.primary(), "primary"));
        }
        if self.runtime.replica_ready() {
            return self.topology.replica().map(|pool| (pool, "replica"));
        }
        match self.runtime.replica_fallback {
            ReplicaFallback::Primary => Some((self.topology.primary(), "primary")),
            ReplicaFallback::FailReadiness => None,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn runtime(&self) -> &ShardRuntime {
        &self.runtime
    }
}

impl std::fmt::Debug for Shard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shard")
            .field("name", &self.name)
            .field("id", &self.id)
            .field("slots", &self.slots)
            .finish_non_exhaustive()
    }
}

struct ShardSetInner {
    shards: Vec<Shard>,
    by_name: HashMap<String, usize>,
    /// `slot_map[slot]` is the index into `shards` of the slot's owner.
    slot_map: Vec<usize>,
    slot_count: u16,
    router: Arc<dyn ShardRouter>,
}

/// The configured set of shards plus the routing strategy.
///
/// Cheap to clone (a single `Arc`). Available from
/// [`AppState::shards`](crate::AppState::shards) and through the
/// [`Shards`] extractor.
#[derive(Clone)]
pub struct ShardSet {
    inner: Arc<ShardSetInner>,
}

impl ShardSet {
    /// Number of configured shards.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.shards.len()
    }

    /// Whether the set contains no shards.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.shards.is_empty()
    }

    /// Number of logical slots (`database.slot_count`).
    #[must_use]
    pub fn slot_count(&self) -> u16 {
        self.inner.slot_count
    }

    /// Shard by positional id.
    #[must_use]
    pub fn get(&self, id: ShardId) -> Option<&Shard> {
        self.inner.shards.get(id.0)
    }

    /// Shard by configured name.
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&Shard> {
        self.inner
            .by_name
            .get(name)
            .and_then(|&idx| self.inner.shards.get(idx))
    }

    /// Iterate shards in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &Shard> {
        self.inner.shards.iter()
    }

    /// Map a routing key onto its logical slot (deterministic hash; see
    /// the module docs for the permanence guarantee).
    #[must_use]
    pub fn slot_for_key<'k>(&self, key: impl Into<ShardKey<'k>>) -> SlotId {
        slot_for_key(key.into(), self.inner.slot_count)
    }

    /// Owner of a logical slot per the configured slot map.
    #[must_use]
    pub fn shard_for_slot(&self, slot: SlotId) -> Option<&Shard> {
        self.inner
            .slot_map
            .get(usize::from(slot.0))
            .and_then(|&idx| self.inner.shards.get(idx))
    }

    /// Resolve the shard that owns `key` via the installed
    /// [`ShardRouter`].
    ///
    /// # Errors
    ///
    /// Returns the router's error, or an internal error if the router
    /// produced an out-of-range [`ShardId`].
    pub async fn route<'k>(&self, key: impl Into<ShardKey<'k>>) -> Result<&Shard, AutumnError> {
        let key = key.into();
        let id = self.inner.router.route(key, self).await?;
        self.get(id).ok_or_else(|| {
            AutumnError::service_unavailable_msg(format!(
                "shard router returned out-of-range shard id {} (have {} shards)",
                id.0,
                self.len()
            ))
        })
    }

    /// Total configured `max_size` across every pool in the set
    /// (primaries plus replicas). Logged at startup so N-shard
    /// deployments notice multiplied connection counts.
    #[must_use]
    pub fn total_max_connections(&self) -> usize {
        self.inner
            .shards
            .iter()
            .map(|shard| {
                shard.topology().primary().status().max_size
                    + shard
                        .topology()
                        .replica()
                        .map_or(0, |pool| pool.status().max_size)
            })
            .sum()
    }
}

impl std::fmt::Debug for ShardSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShardSet")
            .field("shards", &self.inner.shards)
            .field("slot_count", &self.inner.slot_count)
            .finish_non_exhaustive()
    }
}

// ── Construction ─────────────────────────────────────────────────────────────

/// Error building a [`ShardSet`] from configuration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShardSetBuildError {
    /// A shard's connection pool could not be constructed.
    #[error("failed to build pool for shard {shard:?}: {source}")]
    Pool {
        /// Name of the failing shard.
        shard: String,
        /// Underlying pool construction error.
        source: PoolError,
    },
    /// The slot map could not be resolved from configuration.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A custom provider returned the wrong number of shard topologies.
    #[error("expected {expected} shard topologies, got {actual}")]
    TopologyCountMismatch {
        /// Number of configured shards.
        expected: usize,
        /// Number of topologies supplied.
        actual: usize,
    },
}

/// Build a [`ShardSet`] from configuration using the default deadpool
/// factory for every shard topology.
///
/// Returns `Ok(None)` when no `[[database.shards]]` entries are
/// configured.
///
/// # Errors
///
/// Returns [`ShardSetBuildError`] when a pool cannot be constructed or
/// the slot map is invalid.
pub fn create_shard_set(
    config: &DatabaseConfig,
    router: Arc<dyn ShardRouter>,
) -> Result<Option<ShardSet>, ShardSetBuildError> {
    if !config.has_shards() {
        return Ok(None);
    }
    let topologies = config
        .shards
        .iter()
        .map(|shard| {
            crate::db::create_shard_topology(shard, config).map_err(|source| {
                ShardSetBuildError::Pool {
                    shard: shard.name.clone(),
                    source,
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    build_shard_set(config, topologies, router).map(Some)
}

/// Assemble a [`ShardSet`] from pre-built topologies (one per configured
/// shard, in declaration order). Used by custom
/// [`DatabasePoolProvider`](crate::db::DatabasePoolProvider)s and tests.
///
/// # Errors
///
/// Returns [`ShardSetBuildError`] when the topology count does not match
/// the configuration or the slot map is invalid.
pub fn build_shard_set(
    config: &DatabaseConfig,
    topologies: Vec<DatabaseTopology>,
    router: Arc<dyn ShardRouter>,
) -> Result<ShardSet, ShardSetBuildError> {
    if topologies.len() != config.shards.len() {
        return Err(ShardSetBuildError::TopologyCountMismatch {
            expected: config.shards.len(),
            actual: topologies.len(),
        });
    }
    let slot_map = config.resolved_slot_map()?;

    let mut slots_per_shard: Vec<Vec<u16>> = vec![Vec::new(); config.shards.len()];
    for (slot, &owner) in slot_map.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        slots_per_shard[owner].push(slot as u16);
    }

    let shards: Vec<Shard> = config
        .shards
        .iter()
        .zip(topologies)
        .enumerate()
        .map(|(idx, (shard_config, topology))| {
            let replica_configured = topology.replica().is_some();
            Shard {
                name: Arc::from(shard_config.name.as_str()),
                id: ShardId(idx),
                slots: Arc::from(std::mem::take(&mut slots_per_shard[idx])),
                topology,
                runtime: Arc::new(ShardRuntime::new(
                    shard_config.effective_replica_fallback(config),
                    replica_configured,
                )),
            }
        })
        .collect();
    let by_name = shards
        .iter()
        .enumerate()
        .map(|(idx, shard)| (shard.name().to_owned(), idx))
        .collect();

    Ok(ShardSet {
        inner: Arc::new(ShardSetInner {
            shards,
            by_name,
            slot_map,
            slot_count: config.slot_count,
            router,
        }),
    })
}

// ── Health ───────────────────────────────────────────────────────────────────

/// Framework health indicator registered per shard as `db:shard:<name>`.
///
/// Mirrors the control topology's lifecycle: on every readiness probe it
/// snapshots the primary pool, live-checks replica connectivity, and
/// re-runs the migration parity comparison, feeding the shard's runtime
/// state (which gates [`Shard::read_pool`]).
///
/// Reports `Down` — gating `/ready` — only when the shard's replica is
/// unready **and** its `replica_fallback` is `fail_readiness`; a
/// `primary`-fallback shard degrades to primary reads and stays `Up`
/// with the replica state in its details.
pub(crate) struct ShardHealthIndicator {
    shard: Shard,
}

impl ShardHealthIndicator {
    pub(crate) const fn new(shard: Shard) -> Self {
        Self { shard }
    }

    async fn refresh_replica_readiness(&self) {
        let Some(replica_pool) = self.shard.replica_pool() else {
            return;
        };
        // Connectivity goes through the deadpool pool (cheap, reused
        // connections) and runs on every probe; the parity comparison
        // opens fresh connections to both roles and is throttled.
        match replica_pool.get().await {
            Ok(conn) => {
                drop(conn);
                self.shard.runtime().mark_replica_connection_ready();
                if self.shard.runtime().parity_check_due()
                    && let Some((primary_url, replica_url)) = self.shard.runtime().migration_check()
                {
                    let readiness = crate::migrate::check_replica_migration_readiness_blocking(
                        primary_url,
                        replica_url,
                    )
                    .await;
                    if readiness.is_ready() {
                        self.shard.runtime().mark_replica_migrations_ready();
                    } else if let Some(detail) = readiness.detail() {
                        self.shard.runtime().mark_replica_migrations_unready(detail);
                    }
                }
            }
            Err(error) => self
                .shard
                .runtime()
                .mark_replica_connection_unready(format!("replica connection failed: {error}")),
        }
    }
}

impl crate::actuator::HealthIndicator for ShardHealthIndicator {
    fn check(&self) -> futures::future::BoxFuture<'_, crate::actuator::HealthCheckOutput> {
        Box::pin(async move {
            self.refresh_replica_readiness().await;

            let mut details = HashMap::new();
            let status = self.shard.primary_pool().status();
            details.insert("pool_size".to_owned(), serde_json::json!(status.max_size));
            details.insert(
                "active_connections".to_owned(),
                serde_json::json!((status.max_size as u64).saturating_sub(status.available as u64)),
            );
            details.insert(
                "idle_connections".to_owned(),
                serde_json::json!(status.available),
            );
            details.insert(
                "slots".to_owned(),
                serde_json::json!(self.shard.slots().len()),
            );
            if self.shard.replica_pool().is_some() {
                details.insert(
                    "replica_ready".to_owned(),
                    serde_json::json!(self.shard.runtime().replica_ready()),
                );
                if let Some(detail) = self.shard.runtime().detail() {
                    details.insert("replica_detail".to_owned(), serde_json::json!(detail));
                }
            }

            // `read_pool()` is `None` exactly when the replica is unready
            // under `fail_readiness` — the only state that gates `/ready`.
            let output = if self.shard.read_pool().is_some() {
                crate::actuator::HealthCheckOutput::up()
            } else {
                crate::actuator::HealthCheckOutput::down()
            };
            output.with_details(details)
        })
    }
}

/// Register one `db:shard:<name>` readiness indicator per configured
/// shard onto `registry`. Called once at startup by `build_state`.
pub(crate) fn register_shard_health_indicators(
    set: &ShardSet,
    registry: &crate::actuator::HealthIndicatorRegistry,
) {
    for shard in set.iter() {
        let name = format!("db:shard:{}", shard.name());
        if let Err(error) = registry.register(
            name,
            crate::actuator::IndicatorGroup::Readiness,
            Arc::new(ShardHealthIndicator::new(shard.clone())),
        ) {
            tracing::warn!("{error}");
        }
    }
}

// ── Extractors ───────────────────────────────────────────────────────────────

/// Request-extension escape hatch for [`ShardedDb`] key resolution.
///
/// Insert this from middleware (or tests) to route a request to a
/// specific shard key, bypassing tenant extraction:
///
/// ```rust,ignore
/// request.extensions_mut().insert(ShardKeyOverride("tenant-42".to_owned()));
/// ```
#[derive(Debug, Clone)]
pub struct ShardKeyOverride(pub String);

/// Explicit shard access extractor.
///
/// Extract once, then route per call. Captures the request's database
/// context (route-level statement timeout, metrics key, interceptors) at
/// extraction so every checkout carries the same instrumentation as the
/// plain [`Db`](crate::db::Db) extractor.
///
/// Rejects with `503 Service Unavailable` when no `[[database.shards]]`
/// are configured.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/users/{user_id}/bookmarks")]
/// async fn list(shards: Shards, Path(user_id): Path<i64>) -> AutumnResult<&'static str> {
///     let mut db = shards.db_for(user_id).await?;
///     // run Diesel queries against the owning shard's primary
///     Ok("ok")
/// }
/// ```
pub struct Shards {
    set: ShardSet,
    ctx: crate::db::RequestDbContext,
}

impl<S> axum::extract::FromRequestParts<S> for Shards
where
    S: crate::db::DbState + Send + Sync,
{
    type Rejection = AutumnError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let set = state.shards().cloned().ok_or_else(no_shards_configured)?;
        let ctx = crate::db::RequestDbContext::from_parts(parts, state);
        Ok(Self { set, ctx })
    }
}

fn no_shards_configured() -> AutumnError {
    AutumnError::service_unavailable_msg(
        "No shards configured: declare [[database.shards]] in autumn.toml \
         (see docs/guide/sharding.md)",
    )
}

/// How many shards `each_shard` queries concurrently.
const FAN_OUT_CONCURRENCY: usize = 8;

impl Shards {
    /// The underlying [`ShardSet`].
    #[must_use]
    pub const fn set(&self) -> &ShardSet {
        &self.set
    }

    /// Iterate shards in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &Shard> {
        self.set.iter()
    }

    /// Check out a connection to the **primary** of the shard that owns
    /// `key`.
    ///
    /// # Errors
    ///
    /// Returns the router's error or a checkout failure.
    pub async fn db_for<'k>(
        &self,
        key: impl Into<ShardKey<'k>>,
    ) -> Result<crate::db::Db, AutumnError> {
        let shard = self.set.route(key).await?;
        self.checkout_primary(shard).await
    }

    /// Check out a **read** connection to the shard that owns `key`,
    /// honoring the shard's replica topology, readiness, and
    /// `replica_fallback`.
    ///
    /// # Errors
    ///
    /// Returns the router's error, a checkout failure, or
    /// `503 Service Unavailable` when the shard's replica is unready and
    /// its fallback is `fail_readiness`.
    pub async fn read_for<'k>(
        &self,
        key: impl Into<ShardKey<'k>>,
    ) -> Result<crate::db::Db, AutumnError> {
        let shard = self.set.route(key).await?;
        let (pool, role) = shard.read_pool_with_role().ok_or_else(|| {
            AutumnError::service_unavailable_msg(format!(
                "shard {:?} replica is not ready and replica_fallback = \"fail_readiness\"",
                shard.name()
            ))
        })?;
        self.checkout(shard, pool, role).await
    }

    /// Check out a connection to a shard's primary **by name** —
    /// intended for admin/operational paths, not request routing.
    ///
    /// # Errors
    ///
    /// Returns a bad-request error for an unknown name, or a checkout
    /// failure.
    pub async fn db_on(&self, shard_name: &str) -> Result<crate::db::Db, AutumnError> {
        let shard = self
            .set
            .by_name(shard_name)
            .ok_or_else(|| AutumnError::bad_request_msg(format!("unknown shard {shard_name:?}")))?;
        self.checkout_primary(shard).await
    }

    /// Run `f` against the primary of **every** shard, concurrently
    /// (bounded), collecting per-shard results in declaration order.
    ///
    /// Failures are collected rather than short-circuited so aggregate/
    /// admin endpoints can report partial outages. Fan-out latency is
    /// roughly the slowest shard, not the sum — but remember that
    /// scatter/gather amplifies tail latency: the more shards, the more
    /// likely one is slow.
    ///
    /// There are **no cross-shard transactions**: each closure invocation
    /// commits or fails independently, and concurrent writers mean the
    /// collected results can observe torn aggregates.
    ///
    /// The returned future cannot borrow the `&Shard` argument — copy
    /// what you need (e.g. `shard.name().to_owned()`) before the
    /// `async move` block:
    ///
    /// ```rust,ignore
    /// let counts = shards
    ///     .each_shard(|shard, mut db| {
    ///         let name = shard.name().to_owned();
    ///         async move { /* query with db, label with name */ Ok(0i64) }
    ///     })
    ///     .await;
    /// ```
    pub async fn each_shard<T, Fut, F>(&self, f: F) -> Vec<(ShardId, Result<T, AutumnError>)>
    where
        T: Send,
        Fut: std::future::Future<Output = Result<T, AutumnError>> + Send,
        F: Fn(&Shard, crate::db::Db) -> Fut + Send + Sync,
    {
        // FuturesUnordered keeps the pipeline full at FAN_OUT_CONCURRENCY
        // (no head-of-line blocking on a slow shard); results are placed
        // by ShardId so declaration order is preserved. Futures come from
        // a named async fn rather than a closure returning an async block,
        // which would trip rustc #89976 when the handler future is checked
        // for Send.
        use futures::StreamExt as _;

        let mut results: Vec<Option<(ShardId, Result<T, AutumnError>)>> =
            std::iter::repeat_with(|| None)
                .take(self.set.len())
                .collect();
        let mut in_flight: futures::stream::FuturesUnordered<
            futures::future::BoxFuture<'_, (ShardId, Result<T, AutumnError>)>,
        > = futures::stream::FuturesUnordered::new();

        for shard in self.set.iter() {
            if in_flight.len() >= FAN_OUT_CONCURRENCY
                && let Some((id, result)) = in_flight.next().await
            {
                results[id.0] = Some((id, result));
            }
            in_flight.push(Box::pin(self.run_on_shard(shard, &f)));
        }
        while let Some((id, result)) = in_flight.next().await {
            results[id.0] = Some((id, result));
        }
        results.into_iter().flatten().collect()
    }

    async fn run_on_shard<T, Fut, F>(
        &self,
        shard: &Shard,
        f: &F,
    ) -> (ShardId, Result<T, AutumnError>)
    where
        T: Send,
        Fut: std::future::Future<Output = Result<T, AutumnError>> + Send,
        F: Fn(&Shard, crate::db::Db) -> Fut + Send + Sync,
    {
        let result = match self.checkout_primary(shard).await {
            Ok(db) => f(shard, db).await,
            Err(error) => Err(error),
        };
        (shard.id(), result)
    }

    async fn checkout_primary(&self, shard: &Shard) -> Result<crate::db::Db, AutumnError> {
        self.checkout(shard, shard.primary_pool(), "primary").await
    }

    async fn checkout(
        &self,
        shard: &Shard,
        pool: &Pool<AsyncPgConnection>,
        role: &str,
    ) -> Result<crate::db::Db, AutumnError> {
        let ctx = self.ctx.clone();
        crate::db::Db::checkout(crate::db::DbCheckoutParams {
            pool,
            pool_name: &format!("shard:{}:{role}", shard.name()),
            shard: Some(shard.name()),
            statement_timeout: ctx.statement_timeout,
            // Tag the route metric with the shard so per-shard latency
            // separates in /actuator/metrics.
            route_key: ctx
                .route_key
                .map(|key| format!("{key} shard={}", shard.name())),
            metrics: ctx.metrics,
            slow_query_threshold: ctx.slow_query_threshold,
            interceptors: ctx.interceptors,
        })
        .await
    }
}

/// Tenant-routed shard connection extractor.
///
/// Resolves the routing key automatically and checks out a connection to
/// the owning shard's primary. Key resolution order:
///
/// 1. a [`ShardKeyOverride`] request extension (middleware/test escape
///    hatch),
/// 2. the tenant id established by the tenancy middleware
///    ([`tenancy::CURRENT_TENANT`](crate::tenancy::CURRENT_TENANT)),
/// 3. direct tenant extraction from the request per the `[tenancy]`
///    configuration.
///
/// Dereferences to `AsyncPgConnection` exactly like
/// [`Db`](crate::db::Db), and exposes [`tx`](Self::tx) with the same
/// transaction semantics.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/bookmarks")]
/// async fn list(mut db: ShardedDb) -> AutumnResult<String> {
///     // queries run on the tenant's shard
///     Ok(format!("served from shard {}", db.shard()))
/// }
/// ```
pub struct ShardedDb {
    db: crate::db::Db,
    shard_name: Arc<str>,
    shard_id: ShardId,
}

impl ShardedDb {
    /// Name of the shard this connection belongs to.
    #[must_use]
    pub fn shard(&self) -> &str {
        &self.shard_name
    }

    /// Id of the shard this connection belongs to.
    #[must_use]
    pub const fn shard_id(&self) -> ShardId {
        self.shard_id
    }

    /// Connection-scoped tracing span (see [`Db::span`](crate::db::Db::span)).
    #[must_use]
    pub const fn span(&self) -> &tracing::Span {
        self.db.span()
    }

    /// Run an async closure inside a transaction **on this shard**.
    /// Same semantics as [`Db::tx`](crate::db::Db::tx); the transaction
    /// never spans shards.
    ///
    /// # Errors
    ///
    /// See [`Db::tx`](crate::db::Db::tx).
    pub async fn tx<'a, T, E, F>(&'a mut self, f: F) -> Result<T, AutumnError>
    where
        T: Send + 'a,
        E: From<diesel::result::Error> + Send + Sync + 'a,
        AutumnError: From<E>,
        F: for<'r> FnOnce(
                &'r mut crate::db::PooledConnection,
            ) -> scoped_futures::ScopedBoxFuture<'a, 'r, Result<T, E>>
            + Send
            + 'a,
    {
        self.db.tx(f).await
    }

    /// Borrow the underlying [`Db`](crate::db::Db) (e.g. to pass to
    /// helpers written against the unsharded extractor).
    pub const fn db_mut(&mut self) -> &mut crate::db::Db {
        &mut self.db
    }
}

impl std::ops::Deref for ShardedDb {
    type Target = AsyncPgConnection;
    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl std::ops::DerefMut for ShardedDb {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}

impl AsMut<crate::db::Db> for ShardedDb {
    fn as_mut(&mut self) -> &mut crate::db::Db {
        &mut self.db
    }
}

impl axum::extract::FromRequestParts<crate::AppState> for ShardedDb {
    type Rejection = AutumnError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &crate::AppState,
    ) -> Result<Self, Self::Rejection> {
        let shards = Shards::from_request_parts(parts, state).await?;
        let key = resolve_shard_key(parts, state).await?;

        let shard = shards.set.route(&key).await?;
        let shard_name = Arc::clone(&shard.name);
        let shard_id = shard.id();
        let db = shards.checkout_primary(shard).await?;
        Ok(Self {
            db,
            shard_name,
            shard_id,
        })
    }
}

/// Resolve the routing key for [`ShardedDb`]; see its docs for the
/// resolution order.
async fn resolve_shard_key(
    parts: &mut axum::http::request::Parts,
    state: &crate::AppState,
) -> Result<String, AutumnError> {
    if let Some(overridden) = parts.extensions.get::<ShardKeyOverride>() {
        return Ok(overridden.0.clone());
    }
    if let Ok(Some(tenant)) = crate::tenancy::CURRENT_TENANT.try_with(std::clone::Clone::clone) {
        return Ok(tenant);
    }
    let config = state
        .extension::<crate::config::AutumnConfig>()
        .ok_or_else(|| AutumnError::service_unavailable_msg("Config is not available"))?;
    crate::tenancy::extract_tenant_from_parts(parts, &config)
        .await
        .map_err(|error| {
            AutumnError::bad_request_msg(format!(
                "ShardedDb could not resolve a shard key: {error}. Enable [tenancy] so \
                 the tenant id can route the request, or insert a ShardKeyOverride \
                 request extension from middleware (see docs/guide/sharding.md)"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ShardConfig, SlotSpec};

    fn shard_config(name: &str) -> ShardConfig {
        ShardConfig {
            name: name.to_owned(),
            primary_url: format!("postgres://localhost/{name}"),
            slots: None,
            replica_url: None,
            primary_pool_size: None,
            replica_pool_size: None,
            replica_fallback: None,
        }
    }

    fn sharded_config(names: &[&str]) -> DatabaseConfig {
        DatabaseConfig {
            shards: names.iter().map(|name| shard_config(name)).collect(),
            ..Default::default()
        }
    }

    fn shard_set(names: &[&str]) -> ShardSet {
        create_shard_set(&sharded_config(names), Arc::new(HashShardRouter))
            .expect("lazy pools should build")
            .expect("shards configured")
    }

    // ── key→slot golden vectors ─────────────────────────────────────────
    //
    // These values are a PERMANENT CONTRACT. If one of these assertions
    // fails, the change re-routes every existing sharded deployment's
    // keys — do not update the expected values; fix the hash instead.

    #[test]
    fn golden_vector_str_keys_at_64_slots() {
        // Expected slots computed independently (Python reference
        // implementation of FNV-1a 64 mod 64) when the contract was
        // established.
        let cases: &[(&str, u16)] = &[
            ("tenant-1", 11),
            ("tenant-2", 62),
            ("tenant-3", 49),
            ("acme-corp", 2),
            ("globex", 46),
            ("initech", 1),
            ("hooli", 6),
            ("", 37),
            ("a", 12),
            ("00000000-0000-0000-0000-000000000001", 62),
        ];
        for (key, expected_slot) in cases {
            assert_eq!(
                slot_for_key(ShardKey::Str(key), 64),
                SlotId(*expected_slot),
                "key {key:?} must keep routing to slot {expected_slot} forever",
            );
        }
    }

    #[test]
    fn golden_vector_int_keys_at_64_slots() {
        // Expected slots computed independently (Python reference
        // implementation of splitmix64 mod 64) when the contract was
        // established.
        let cases: &[(i64, u16)] = &[
            (0, 47),
            (1, 1),
            (2, 14),
            (42, 21),
            (1_000_000, 39),
            (-1, 32),
            (i64::MAX, 39),
            (i64::MIN, 27),
        ];
        for (key, expected_slot) in cases {
            assert_eq!(
                slot_for_key(ShardKey::Int(*key), 64),
                SlotId(*expected_slot),
                "key {key} must keep routing to slot {expected_slot} forever",
            );
        }
    }

    #[test]
    fn golden_vector_bytes_match_equivalent_str() {
        // Str and Bytes share FNV-1a, so identical bytes route identically.
        assert_eq!(
            slot_for_key(ShardKey::Bytes(b"tenant-1"), 64),
            slot_for_key(ShardKey::Str("tenant-1"), 64),
        );
    }

    #[test]
    fn slots_stay_in_range_and_spread_roughly_uniformly() {
        let slot_count = 16u16;
        let mut histogram = vec![0usize; usize::from(slot_count)];
        for i in 0..10_000i64 {
            let slot = slot_for_key(ShardKey::Int(i), slot_count);
            assert!(slot.0 < slot_count);
            histogram[usize::from(slot.0)] += 1;
        }
        let expected = 10_000 / usize::from(slot_count);
        for (slot, count) in histogram.iter().enumerate() {
            assert!(
                *count > expected / 2 && *count < expected * 2,
                "slot {slot} has {count} keys (expected ≈{expected})"
            );
        }
    }

    // ── ShardSet behavior ───────────────────────────────────────────────

    #[tokio::test]
    async fn route_is_deterministic_and_respects_slot_map() {
        let mut config = sharded_config(&["a", "b"]);
        config.slot_count = 4;
        config.shards[0].slots = Some(vec![SlotSpec::Range("0-2".to_owned())]);
        config.shards[1].slots = Some(vec![SlotSpec::Index(3)]);
        let set = create_shard_set(&config, Arc::new(HashShardRouter))
            .expect("build")
            .expect("configured");

        for key in ["k1", "k2", "k3", "k4", "k5"] {
            let slot = set.slot_for_key(key);
            let expected = if slot.0 == 3 { "b" } else { "a" };
            let routed = set.route(key).await.expect("route");
            assert_eq!(routed.name(), expected, "key {key:?} slot {}", slot.0);
            // Same key always lands on the same shard.
            assert_eq!(set.route(key).await.expect("route").id(), routed.id());
        }
    }

    #[tokio::test]
    async fn moving_a_slot_in_config_moves_only_that_slot() {
        // "Reshard" by reassigning slot 3 from shard b to a new shard c:
        // keys in slots 0-2 must not move.
        let mut before = sharded_config(&["a", "b"]);
        before.slot_count = 4;
        before.shards[0].slots = Some(vec![SlotSpec::Range("0-2".to_owned())]);
        before.shards[1].slots = Some(vec![SlotSpec::Index(3)]);

        let mut after = sharded_config(&["a", "b", "c"]);
        after.slot_count = 4;
        after.shards[0].slots = Some(vec![SlotSpec::Range("0-2".to_owned())]);
        after.shards[1].slots = Some(vec![]);
        after.shards[2].slots = Some(vec![SlotSpec::Index(3)]);

        let set_before = create_shard_set(&before, Arc::new(HashShardRouter))
            .expect("build")
            .expect("configured");
        let set_after = create_shard_set(&after, Arc::new(HashShardRouter))
            .expect("build")
            .expect("configured");

        for i in 0..200i64 {
            let slot = set_before.slot_for_key(i);
            assert_eq!(slot, set_after.slot_for_key(i), "key→slot never changes");
            let before_shard = set_before.route(i).await.expect("route");
            let after_shard = set_after.route(i).await.expect("route");
            if slot.0 == 3 {
                assert_eq!(before_shard.name(), "b");
                assert_eq!(after_shard.name(), "c");
            } else {
                assert_eq!(before_shard.name(), "a");
                assert_eq!(after_shard.name(), "a");
            }
        }
    }

    #[test]
    fn by_name_and_get_resolve_shards() {
        let set = shard_set(&["alpha", "beta"]);
        assert_eq!(set.len(), 2);
        assert_eq!(set.by_name("beta").expect("beta").id(), ShardId(1));
        assert_eq!(set.get(ShardId(0)).expect("alpha").name(), "alpha");
        assert!(set.by_name("gamma").is_none());
        assert!(set.get(ShardId(9)).is_none());
        let names: Vec<&str> = set.iter().map(Shard::name).collect();
        assert_eq!(names, ["alpha", "beta"]);
    }

    #[test]
    fn auto_split_assigns_contiguous_slots() {
        let set = shard_set(&["a", "b"]);
        assert_eq!(set.slot_count(), 64);
        assert_eq!(set.get(ShardId(0)).expect("a").slots().len(), 32);
        assert_eq!(
            set.get(ShardId(1)).expect("b").slots(),
            (32..64).collect::<Vec<u16>>()
        );
    }

    #[test]
    fn create_shard_set_returns_none_without_shards() {
        let config = DatabaseConfig::default();
        assert!(
            create_shard_set(&config, Arc::new(HashShardRouter))
                .expect("ok")
                .is_none()
        );
    }

    #[test]
    fn build_shard_set_rejects_topology_count_mismatch() {
        let config = sharded_config(&["a", "b"]);
        let result = build_shard_set(&config, Vec::new(), Arc::new(HashShardRouter));
        assert!(matches!(
            result,
            Err(ShardSetBuildError::TopologyCountMismatch {
                expected: 2,
                actual: 0
            })
        ));
    }

    // ── read_pool / replica fallback semantics ──────────────────────────

    fn shard_with_replica(fallback: ReplicaFallback) -> Shard {
        let mut config = sharded_config(&["a"]);
        config.shards[0].replica_url = Some("postgres://localhost/a_ro".to_owned());
        config.shards[0].replica_fallback = Some(fallback);
        let set = create_shard_set(&config, Arc::new(HashShardRouter))
            .expect("build")
            .expect("configured");
        set.get(ShardId(0)).expect("shard").clone()
    }

    #[test]
    fn read_pool_uses_primary_when_no_replica() {
        let set = shard_set(&["a"]);
        let shard = set.get(ShardId(0)).expect("shard");
        assert!(shard.read_pool().is_some());
        assert!(shard.replica_pool().is_none());
    }

    #[test]
    fn read_pool_requires_readiness_check_before_replica_traffic() {
        let shard = shard_with_replica(ReplicaFallback::Primary);
        // Unchecked replica: fallback policy routes reads to the primary.
        assert!(shard.read_pool().is_some());
        assert!(shard.runtime().detail().is_some());

        shard.runtime().mark_replica_connection_ready();
        assert!(shard.runtime().replica_ready());
        assert!(shard.read_pool().is_some());
        assert!(shard.runtime().detail().is_none());
    }

    #[test]
    fn read_pool_fails_closed_under_fail_readiness() {
        let shard = shard_with_replica(ReplicaFallback::FailReadiness);
        assert!(
            shard.read_pool().is_none(),
            "unchecked replica fails closed"
        );

        shard.runtime().mark_replica_connection_ready();
        assert!(shard.read_pool().is_some());

        shard
            .runtime()
            .mark_replica_migrations_unready("replica lags primary");
        assert!(shard.read_pool().is_none());
        assert!(shard.runtime().detail().expect("detail").contains("lags"));
    }

    // ── per-shard health indicator ──────────────────────────────────────

    fn shard_with_unreachable_replica(fallback: ReplicaFallback) -> Shard {
        let mut config = sharded_config(&["a"]);
        // Nothing listens on these URLs; keep the failing checks fast.
        config.connect_timeout_secs = 1;
        config.shards[0].replica_url = Some("postgres://localhost:1/a_ro".to_owned());
        config.shards[0].replica_fallback = Some(fallback);
        let set = create_shard_set(&config, Arc::new(HashShardRouter))
            .expect("build")
            .expect("configured");
        set.get(ShardId(0)).expect("shard").clone()
    }

    #[tokio::test]
    async fn shard_indicator_gates_readiness_for_fail_readiness_replica() {
        use crate::actuator::HealthIndicator as _;

        let shard = shard_with_unreachable_replica(ReplicaFallback::FailReadiness);
        let indicator = ShardHealthIndicator::new(shard);
        let output = indicator.check().await;

        assert!(
            !output.status.is_healthy(),
            "unreachable replica under fail_readiness must report Down"
        );
        assert_eq!(output.details["replica_ready"], serde_json::json!(false));
        assert!(output.details.contains_key("replica_detail"));
    }

    #[tokio::test]
    async fn shard_indicator_stays_up_for_primary_fallback_replica() {
        use crate::actuator::HealthIndicator as _;

        let shard = shard_with_unreachable_replica(ReplicaFallback::Primary);
        let indicator = ShardHealthIndicator::new(shard);
        let output = indicator.check().await;

        assert!(
            output.status.is_healthy(),
            "primary fallback degrades to primary reads and must stay Up"
        );
        assert_eq!(output.details["replica_ready"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn register_shard_health_indicators_names_components() {
        let set = shard_set(&["alpha", "beta"]);
        let registry = crate::actuator::HealthIndicatorRegistry::new();

        register_shard_health_indicators(&set, &registry);
        // Re-registration is ignored with a warning rather than panicking.
        register_shard_health_indicators(&set, &registry);

        let results = registry.run_all().await;
        let mut names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["db:shard:alpha", "db:shard:beta"]);
        assert!(
            results
                .iter()
                .all(|r| matches!(r.group, crate::actuator::IndicatorGroup::Readiness)),
            "shard indicators gate readiness"
        );
    }

    #[test]
    fn total_max_connections_sums_every_pool() {
        let mut config = sharded_config(&["a", "b"]);
        config.pool_size = 7;
        config.shards[1].replica_url = Some("postgres://localhost/b_ro".to_owned());
        config.shards[1].replica_pool_size = Some(3);
        let set = create_shard_set(&config, Arc::new(HashShardRouter))
            .expect("build")
            .expect("configured");
        // a primary (7) + b primary (7) + b replica (3).
        assert_eq!(set.total_max_connections(), 17);
    }
}
