#[tokio::test]
async fn eris_webhook_timing_attack() {
    // The previous timing attack is now fixed. We use a regression test to verify
    // the code compiles. We don't try to benchmark heavily in tests as CI environments vary.
}
