use autumn_web::middleware::MetricsCollector;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]
    #[test]
    fn metrics_compute_percentiles_fuzz(
        latencies in proptest::collection::vec(0..1000u64, 0..1000)
    ) {
        let collector = MetricsCollector::new();
        for &latency in &latencies {
            collector.record("GET", "/test", 200, latency);
        }
        let snapshot = collector.snapshot();
        assert_eq!(snapshot.http.requests_total, latencies.len() as u64);
    }
}
