use autumn_web::middleware::MetricsCollector;

#[test]
fn test_metrics_unbounded_growth() {
    let collector = MetricsCollector::new();

    // Trigger unbounded memory growth in metrics
    for i in 0..200_000 {
        let route = format!("/api/users/{i}");
        collector.record("GET", &route, 200, 10);
    }

    let snap = collector.snapshot();

    // The snapshot will have 100,000 unique routes, proving the fragility.
    assert!(snap.http.by_route.len() <= 10_000 * 16);
    assert_eq!(snap.http.by_route.len(), 160_000); // Because we insert 200,000 unique routes, it will fill all 16 shards to 10,000 max capacity exactly.
}
