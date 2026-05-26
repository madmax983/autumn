use autumn_web::test::TestApp;
use autumn_web::webhook_outbound::{
    InMemoryOutboundWebhookStore, OutboundWebhookPlugin, OutboundWebhookStore, WebhookSubscription,
    WebhookSubscriptionStatus,
};
use outbound_webhooks_example::User;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn test_outbound_webhook_dispatch_flow() {
    use autumn_web::job::{self, JobInfo, clear_global_job_client, global_job_runtime_test_lock};

    let _guard = global_job_runtime_test_lock().lock().await;
    clear_global_job_client();

    let store = Arc::new(InMemoryOutboundWebhookStore::new());
    let plugin = OutboundWebhookPlugin::new(store.clone()).with_initial_backoff_ms(1);

    // Setup TestApp with outbound webhook plugin and the example's routes
    let mut app_builder = TestApp::new()
        .plugin(plugin)
        .routes(outbound_webhooks_example::routes());

    // Register HTTP mock for external delivery target
    let mock = app_builder
        .http_mock("http://mock-external-receiver/webhook")
        .post("/webhook")
        .respond_with(200, serde_json::json!({ "success": true }));

    let app = app_builder.build();
    let state = app.state();

    let shutdown = tokio_util::sync::CancellationToken::new();
    let config = autumn_web::config::JobConfig::default();
    let job_info = JobInfo {
        name: "autumn_webhook_delivery".to_owned(),
        max_attempts: 1,
        initial_backoff_ms: 1,
        handler: autumn_web::webhook_outbound::deliver_webhook_job,
    };
    job::start_runtime(vec![job_info], state, &shutdown, &config).unwrap();

    // Create a subscription targeting the mock url
    let sub = WebhookSubscription {
        id: "sub_example".to_owned(),
        target_url: "http://mock-external-receiver/webhook".to_owned(),
        event_topics: vec!["user.created".to_owned()],
        secret: "example_signing_secret_key_32_bytes!!".to_owned(),
        status: WebhookSubscriptionStatus::Active,
        consecutive_failures: 0,
    };
    store.create_subscription(sub).await.unwrap();

    // Fire HTTP request to POST /users to register a user
    let user_payload = User {
        id: "usr_100".to_owned(),
        name: "Alice".to_owned(),
        email: "alice@example.com".to_owned(),
    };

    let response = app
        .post("/users")
        .json(&serde_json::json!(user_payload))
        .send()
        .await;

    response.assert_ok();

    // Wait for the background delivery job to execute
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Assert that the mock external receiver was called with the signed request
    mock.expect_called(1);

    // Assert delivery log is stored and successful
    let logs = store.get_delivery_logs().await.unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].response_status, Some(200));
    assert!(!logs[0].is_dlq);

    shutdown.cancel();
    clear_global_job_client();
}
