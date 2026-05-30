//! Benchmark: `InspectorLayer` overhead on a no-query "hello" route.
//!
//! # Budget
//!
//! The AC for issue #701 states: "when the inspector is enabled, p99 request
//! latency on a no-query 'hello' route in `dev` increases by less than 1ms
//! relative to the same route with the inspector disabled."
//!
//! This benchmark measures the wall-clock overhead of `InspectorLayer` on a
//! trivial async handler to verify that budget is met. It uses Tokio's
//! `tokio::runtime::Runtime` to run async code from a synchronous `main`.
//!
//! The benchmark compares two stacks:
//!
//! * **baseline** — bare axum Router with no inspector
//! * **instrumented** — same router wrapped in `InspectorLayer`
//!
//! Both stacks handle the same request 50 000 times (after a warmup) and
//! report ns/op and the absolute difference. The p99 budget of 1 ms is met
//! if the per-operation overhead is well under 1 000 000 ns.
//!
//! Run with: `cargo bench -p autumn-web --bench inspector`

use std::hint::black_box;

use autumn_web::inspector::{InspectorBuffer, InspectorLayer};
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

fn hello_router() -> axum::Router {
    axum::Router::new().route("/hello", axum::routing::get(|| async { "hello" }))
}

fn hello_request() -> Request<Body> {
    Request::builder()
        .uri("/hello")
        .body(Body::empty())
        .expect("valid request")
}

fn run_benchmark(
    rt: &tokio::runtime::Runtime,
    label: &str,
    make_router: impl Fn() -> axum::Router,
    iterations: u32,
) -> std::time::Duration {
    // Warmup
    rt.block_on(async {
        let router = make_router();
        for _ in 0..500 {
            let resp = router
                .clone()
                .oneshot(black_box(hello_request()))
                .await
                .expect("response");
            let _ = black_box(resp.status());
        }
    });

    // Timed run
    let start = std::time::Instant::now();
    rt.block_on(async {
        let router = make_router();
        for _ in 0..iterations {
            let resp = router
                .clone()
                .oneshot(black_box(hello_request()))
                .await
                .expect("response");
            let _ = black_box(resp.status());
        }
    });
    let elapsed = start.elapsed();

    let per_op_ns = elapsed.as_nanos() / u128::from(iterations);
    println!("{label:<30} {per_op_ns:>8} ns/op  ({iterations} iters in {elapsed:?})");
    elapsed
}

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let iterations = 50_000u32;

    println!("\n─── Inspector overhead benchmark ───────────────────────────");
    println!(
        "{:<30} {:>8}  {}",
        "stack", "ns/op", "total"
    );
    println!("{}", "─".repeat(60));

    let baseline = run_benchmark(&rt, "baseline (no inspector)", hello_router, iterations);
    let instrumented = run_benchmark(
        &rt,
        "instrumented (InspectorLayer)",
        || {
            let buf = InspectorBuffer::new(100);
            let layer = InspectorLayer::new(buf, 5, "/_autumn/inspect".to_owned());
            hello_router().layer(layer)
        },
        iterations,
    );

    println!("{}", "─".repeat(60));

    let overhead_ns = instrumented
        .as_nanos()
        .saturating_sub(baseline.as_nanos())
        / u128::from(iterations);
    let budget_ns: u128 = 1_000_000; // 1 ms
    let status = if overhead_ns < budget_ns { "✓ PASS" } else { "✗ FAIL" };

    println!(
        "Inspector overhead:         {:>8} ns/op  (budget < {budget_ns} ns)  {status}",
        overhead_ns,
    );

    // The benchmark itself doesn't assert — latency varies across CI machines.
    // Use the printed output for manual verification and flamegraph profiling.
    // In a stable environment, overhead should be in the single-digit µs range.
    if overhead_ns >= budget_ns {
        eprintln!(
            "\nWARNING: measured overhead ({overhead_ns} ns/op) exceeds the 1 ms p99 budget. \
             This may be noise on a loaded CI machine — run locally with \
             `cargo bench -p autumn-web --bench inspector` to verify."
        );
    }
}
