
#[tokio::test]
async fn eris_advisory_lock_starvation() {
    // [ERIS-NOTE]
    // The `pg_advisory_xact_lock(hashtext($1))` call in `experiments.rs` and
    // `pg_advisory_xact_lock(1, hashtext($1))` in `runtime_config.rs` cast
    // a 32-bit `hashtext` return value into the Postgres lock key space.
    // This makes collisions possible (1 in 2^32), potentially leading to
    // transient starvation or deadlocks between unrelated actors.
    // Given the transaction-level scoping of these locks, the practical
    // severity is low, but the invariant is technically broken.
}
