use autumn_web::auth::{hash_password, verify_password};
use std::time::Instant;

#[tokio::test]
async fn eris_auth_timing_test() {
    // 1. Time the verification of a valid hash
    let valid_hash = hash_password("secret123").await.unwrap();
    let start_valid = Instant::now();
    let _ = verify_password("secret123", &valid_hash).await;
    let elapsed_valid = start_valid.elapsed();

    // 2. Time the verification of an invalid hash
    let start_invalid = Instant::now();
    let _ = verify_password("secret123", "invalid_hash_format").await;
    let elapsed_invalid = start_invalid.elapsed();

    // 3. Compare the times. If invalid is significantly faster, we have a timing vulnerability.
    println!("Elapsed valid: {:?}", elapsed_valid);
    println!("Elapsed invalid: {:?}", elapsed_invalid);

    // To make this a regression test that fails now and passes later, we assert:
    assert!(
        elapsed_invalid.as_millis() >= elapsed_valid.as_millis() / 2,
        "Timing vulnerability detected: Invalid hash check is significantly faster ({}ms vs {}ms)",
        elapsed_invalid.as_millis(),
        elapsed_valid.as_millis()
    );
}
