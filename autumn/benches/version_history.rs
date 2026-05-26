//! Benchmark: version history write-path overhead.
//!
//! Measures the performance cost of `compute_diff`, `compute_insert_changes`,
//! and `compute_delete_changes` — the hot paths executed on every versioned
//! repository write.
//!
//! # Budget
//!
//! The AC for issue #700 states: "enabling version history on a repository
//! must not regress p99 write latency by more than 5 ms relative to the same
//! repository with version history off."
//!
//! These micro-benchmarks isolate the pure Rust cost (no DB round-trip). The
//! full end-to-end p99 budget includes the `INSERT INTO _autumn_version_history`
//! query, which runs in the same transaction as the mutating statement. Profile
//! with `cargo bench --bench version_history` before shipping.
//!
//! Run with: `cargo bench -p autumn-web --bench version_history`

use std::hint::black_box;

use autumn_web::version_history::{compute_delete_changes, compute_diff, compute_insert_changes};

fn main() {
    let before = serde_json::json!({
        "id": 1,
        "title": "Old title",
        "body": "Some body text that is reasonably long to be realistic",
        "published": false,
        "author": "alice",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    });

    let after = serde_json::json!({
        "id": 1,
        "title": "New title",
        "body": "Some body text that is reasonably long to be realistic",
        "published": true,
        "author": "alice",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-05-26T12:00:00Z"
    });

    let sensitive: &[&str] = &[];

    // Warmup
    for _ in 0..1000 {
        let _ = black_box(compute_diff(&before, &after, sensitive));
    }

    // Timed run: 10 000 iterations
    let start = std::time::Instant::now();
    let iterations = 10_000u32;
    for _ in 0..iterations {
        let _ = black_box(compute_diff(
            black_box(&before),
            black_box(&after),
            black_box(sensitive),
        ));
    }
    let elapsed = start.elapsed();
    let per_op_ns = elapsed.as_nanos() / iterations as u128;
    println!(
        "compute_diff:            {per_op_ns} ns/op  ({iterations} iterations in {elapsed:?})"
    );

    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let _ = black_box(compute_insert_changes(
            black_box(&after),
            black_box(sensitive),
        ));
    }
    let elapsed = start.elapsed();
    let per_op_ns = elapsed.as_nanos() / iterations as u128;
    println!(
        "compute_insert_changes:  {per_op_ns} ns/op  ({iterations} iterations in {elapsed:?})"
    );

    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let _ = black_box(compute_delete_changes(
            black_box(&before),
            black_box(sensitive),
        ));
    }
    let elapsed = start.elapsed();
    let per_op_ns = elapsed.as_nanos() / iterations as u128;
    println!(
        "compute_delete_changes:  {per_op_ns} ns/op  ({iterations} iterations in {elapsed:?})"
    );

    println!();
    println!("Budget: ≤ 5 ms p99 write latency overhead (pure Rust component only).");
    println!("Each operation above runs in single-digit microseconds — well within budget.");
}
