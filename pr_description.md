🧨 **The Trigger:** A client disconnects or times out before the `MetricsFuture` resolves to completion.
📉 **The Stack Trace:** N/A (Memory/resource leak instead of a panic). The `requests_active` metric continues to increment indefinitely, leading to permanently skewed active connection tracking.
🧪 **Reproduction:** Run `cargo test --test chaos_metrics_leak -p autumn-web`. The test simulates a dropped future, revealing the counter stays stuck at 1 instead of decrementing.
😈 **Comment:** You assumed every future runs to completion. You were wrong. Dropped futures left active requests dangling forever.
