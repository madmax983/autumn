use autumn_web::middleware::MetricsCollector;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_concurrent_stress() {
    let collector = Arc::new(MetricsCollector::new());
    let mut handles = vec![];

    for _ in 0..1000 {
        let c = collector.clone();
        handles.push(tokio::spawn(async move {
            c.record("GET", "/test", 200, 10);
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}
